//! `BoxMatch` enum and `match_box` dispatcher.

use tlib::{NodeKind, TreeArena};

use crate::BoxId;
use crate::internals::slider_params4;
use crate::tags::*;

/// Canonical structural view returned by [`match_box`].
///
/// This enum is the box-layer counterpart of the signal/FIR match enums used in
/// later phases: it decodes the raw tree-encoded representation into one
/// stable shape vocabulary that callers can pattern-match on without depending
/// on tag strings or child ordering.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BoxMatch<'a> {
    Unknown,
    Ident(&'a str),
    Int(i32),
    Real(f64),
    Wire,
    Cut,
    Seq(BoxId, BoxId),
    Par(BoxId, BoxId),
    Rec(BoxId, BoxId),
    Split(BoxId, BoxId),
    Merge(BoxId, BoxId),
    Appl(BoxId, BoxId),
    Access(BoxId, BoxId),
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    And,
    Or,
    Xor,
    Lsh,
    LRsh,
    Rsh,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    Pow,
    Acos,
    Asin,
    Atan,
    Atan2,
    Cos,
    Sin,
    Tan,
    Exp,
    Exp10,
    Log,
    Log10,
    Sqrt,
    Abs,
    Fmod,
    Remainder,
    Floor,
    Ceil,
    Rint,
    Round,
    Delay,
    Delay1,
    Min,
    Max,
    Prefix,
    IntCast,
    FloatCast,
    ReadOnlyTable,
    WriteReadTable,
    Select2,
    Select3,
    AssertBounds,
    Lowest,
    Highest,
    Attach,
    Enable,
    Control,
    Slot(i32),
    Symbolic(BoxId, BoxId),
    IPar(BoxId, BoxId, BoxId),
    ISeq(BoxId, BoxId, BoxId),
    ISum(BoxId, BoxId, BoxId),
    IProd(BoxId, BoxId, BoxId),
    WithLocalDef(BoxId, BoxId),
    ModifLocalDef(BoxId, BoxId),
    WithRecDef(BoxId, BoxId, BoxId),
    Metadata(BoxId, BoxId),
    Environment,
    Component(BoxId),
    Library(BoxId),
    /// Parser/import node preserving one raw `import("...")` statement.
    ///
    /// Source provenance (C++):
    /// - `compiler/boxes/boxes.cpp`
    /// - `importFile(Tree filename)`
    /// - `isImportFile(Tree s, Tree& filename)`
    ///
    /// This node is intentionally distinct from `component(...)` / `library(...)`.
    /// It survives parsing and definition normalization so later structural
    /// import expansion can replay the C++ `SourceReader::expandList(...)`
    /// boundary on parsed definition trees instead of depending on raw source
    /// preprocessing.
    ImportFile(BoxId),
    Waveform(BoxId),
    Route(BoxId, BoxId, BoxId),
    Ffunction(BoxId, BoxId, BoxId),
    FFun(BoxId),
    FConst(BoxId, BoxId, BoxId),
    FVar(BoxId, BoxId, BoxId),
    Case(BoxId),
    /// Partially-applied pattern matcher stored in an evaluator side-table.
    ///
    /// The single child is a `boxInt(key)` indexing into the evaluator's PM store.
    /// This node exists so that `force_value_to_box` can return a `TreeId` for a
    /// partially-applied `case` expression without re-entering the evaluator.
    ///
    /// # C++ equivalent
    /// `boxPatternMatcher(Automaton*, int state, Tree env, Tree orig, Tree revParList)`
    /// — C++ stores all PM state inline in the tree; Rust keeps it in a side-table.
    PatternMatcher(BoxId),
    /// Closure stored in an evaluator side-table.
    ///
    /// The single child is a `boxInt(key)` indexing into the evaluator's closure store.
    /// This node exists so that `force_value_to_box` can return a `TreeId` for a
    /// closure (abstraction + captured environment) without losing the environment.
    ///
    /// # C++ equivalent
    /// `closure(expr, genv, visited, lenv)` — C++ stores closures inline in the
    /// tree; Rust keeps them in a side-table.
    Closure(BoxId),
    PatternVar(BoxId),
    Abstr(BoxId, BoxId),
    Modulation(BoxId, BoxId),
    Inputs(BoxId),
    Outputs(BoxId),
    /// `cinputs(expr)`: the control inputs of `expr`, folded by eval to the
    /// `par` of its widget boxes. faust-rs extension, no C++ equivalent.
    CInputs(BoxId),
    /// `cinput(index, expr)`: one control input of `expr`, folded by eval to
    /// `(widget, init, min, max, step)`. faust-rs extension.
    CInput(BoxId, BoxId),
    /// `coutputs(expr)`: the bargraphs of `expr`, folded by eval to the `par`
    /// of its bargraph boxes. faust-rs extension.
    COutputs(BoxId),
    /// `coutput(index, expr)`: one bargraph of `expr`, folded by eval to
    /// `(bargraph, min, max)`. faust-rs extension.
    COutput(BoxId, BoxId),
    /// Automatic differentiation wrapper preserving the wrapped block diagram
    /// until the post-eval propagation boundary.
    ///
    /// Source provenance (C++):
    /// - `compiler/boxes/boxes.cpp`
    /// - `boxForwardAD(Tree exp, Tree seed)` — `isBoxForwardAD(Tree t, Tree& exp, Tree& seed)`
    ForwardAD(BoxId, BoxId),
    /// Reverse-mode automatic differentiation wrapper.
    ///
    /// Two-child shape mirroring [`BoxMatch::ForwardAD`]: `(expr, seeds)`.
    /// Both children are normalized through eval and re-validated by
    /// `propagate`, which expands the wrapper into a primal-plus-gradient
    /// signal bundle when reverse-mode support reaches the visited node
    /// family.
    ReverseAD(BoxId, BoxId),
    Ondemand(BoxId),
    Upsampling(BoxId),
    Downsampling(BoxId),
    Button(BoxId),
    Checkbox(BoxId),
    VSlider(BoxId, BoxId, BoxId, BoxId, BoxId),
    HSlider(BoxId, BoxId, BoxId, BoxId, BoxId),
    NumEntry(BoxId, BoxId, BoxId, BoxId, BoxId),
    VGroup(BoxId, BoxId),
    HGroup(BoxId, BoxId),
    TGroup(BoxId, BoxId),
    VBargraph(BoxId, BoxId, BoxId),
    HBargraph(BoxId, BoxId, BoxId),
    Soundfile(BoxId, BoxId),
}

/// Decodes one `BoxId` into canonical [`BoxMatch`] shape.
///
/// Performance note:
/// - The current hot path uses arity-first dispatch (`children.len()`) then tag matching.
/// - A tag-id-only (`u32`) dispatch prototype was benchmarked on this branch and did not
///   improve end-to-end throughput under `match_box_bench` workloads, so this version keeps
///   the best measured implementation for now.
#[must_use]
pub fn match_box<'a>(arena: &'a TreeArena, b: BoxId) -> BoxMatch<'a> {
    let Some(node) = arena.node(b) else {
        return BoxMatch::Unknown;
    };
    match &node.kind {
        NodeKind::Int(v) => match i32::try_from(*v) {
            Ok(v) => BoxMatch::Int(v),
            Err(_) => BoxMatch::Unknown,
        },
        NodeKind::FloatBits(bits) => BoxMatch::Real(f64::from_bits(*bits)),
        NodeKind::Tag(tag) => {
            let tag = arena.tag_name(*tag).unwrap_or("");
            let children = node.children.as_slice();
            match children.len() {
                0 => match tag {
                    BOX_WIRE_TAG => BoxMatch::Wire,
                    BOX_CUT_TAG => BoxMatch::Cut,
                    BOX_ADD_TAG => BoxMatch::Add,
                    BOX_SUB_TAG => BoxMatch::Sub,
                    BOX_MUL_TAG => BoxMatch::Mul,
                    BOX_DIV_TAG => BoxMatch::Div,
                    BOX_REM_TAG => BoxMatch::Rem,
                    BOX_AND_TAG => BoxMatch::And,
                    BOX_OR_TAG => BoxMatch::Or,
                    BOX_XOR_TAG => BoxMatch::Xor,
                    BOX_LSH_TAG => BoxMatch::Lsh,
                    BOX_RSH_TAG => BoxMatch::Rsh,
                    BOX_LRSH_TAG => BoxMatch::LRsh,
                    BOX_LT_TAG => BoxMatch::Lt,
                    BOX_LE_TAG => BoxMatch::Le,
                    BOX_GT_TAG => BoxMatch::Gt,
                    BOX_GE_TAG => BoxMatch::Ge,
                    BOX_EQ_TAG => BoxMatch::Eq,
                    BOX_NE_TAG => BoxMatch::Ne,
                    BOX_POW_TAG => BoxMatch::Pow,
                    BOX_ACOS_TAG => BoxMatch::Acos,
                    BOX_ASIN_TAG => BoxMatch::Asin,
                    BOX_ATAN_TAG => BoxMatch::Atan,
                    BOX_ATAN2_TAG => BoxMatch::Atan2,
                    BOX_COS_TAG => BoxMatch::Cos,
                    BOX_SIN_TAG => BoxMatch::Sin,
                    BOX_TAN_TAG => BoxMatch::Tan,
                    BOX_EXP_TAG => BoxMatch::Exp,
                    BOX_EXP10_TAG => BoxMatch::Exp10,
                    BOX_LOG_TAG => BoxMatch::Log,
                    BOX_LOG10_TAG => BoxMatch::Log10,
                    BOX_SQRT_TAG => BoxMatch::Sqrt,
                    BOX_ABS_TAG => BoxMatch::Abs,
                    BOX_FMOD_TAG => BoxMatch::Fmod,
                    BOX_REMAINDER_TAG => BoxMatch::Remainder,
                    BOX_FLOOR_TAG => BoxMatch::Floor,
                    BOX_CEIL_TAG => BoxMatch::Ceil,
                    BOX_RINT_TAG => BoxMatch::Rint,
                    BOX_ROUND_TAG => BoxMatch::Round,
                    BOX_DELAY_TAG => BoxMatch::Delay,
                    BOX_DELAY1_TAG => BoxMatch::Delay1,
                    BOX_MIN_TAG => BoxMatch::Min,
                    BOX_MAX_TAG => BoxMatch::Max,
                    BOX_PREFIX_TAG => BoxMatch::Prefix,
                    BOX_INT_CAST_TAG => BoxMatch::IntCast,
                    BOX_FLOAT_CAST_TAG => BoxMatch::FloatCast,
                    BOX_READ_ONLY_TABLE_TAG => BoxMatch::ReadOnlyTable,
                    BOX_WRITE_READ_TABLE_TAG => BoxMatch::WriteReadTable,
                    BOX_SELECT2_TAG => BoxMatch::Select2,
                    BOX_SELECT3_TAG => BoxMatch::Select3,
                    BOX_ASSERT_BOUNDS_TAG => BoxMatch::AssertBounds,
                    BOX_LOWEST_TAG => BoxMatch::Lowest,
                    BOX_HIGHEST_TAG => BoxMatch::Highest,
                    BOX_ATTACH_TAG => BoxMatch::Attach,
                    BOX_ENABLE_TAG => BoxMatch::Enable,
                    BOX_CONTROL_TAG => BoxMatch::Control,
                    BOX_ENVIRONMENT_TAG => BoxMatch::Environment,
                    _ => BoxMatch::Unknown,
                },
                1 => {
                    let c0 = children[0];
                    match tag {
                        BOX_IDENT_TAG => match arena.kind(c0) {
                            Some(NodeKind::Symbol(name)) => BoxMatch::Ident(name.as_ref()),
                            _ => BoxMatch::Unknown,
                        },
                        BOX_SLOT_TAG => match arena.kind(c0) {
                            Some(NodeKind::Int(v)) => match i32::try_from(*v) {
                                Ok(v) => BoxMatch::Slot(v),
                                Err(_) => BoxMatch::Unknown,
                            },
                            _ => BoxMatch::Unknown,
                        },
                        BOX_COMPONENT_TAG => BoxMatch::Component(c0),
                        BOX_LIBRARY_TAG => BoxMatch::Library(c0),
                        IMPORT_FILE_TAG => BoxMatch::ImportFile(c0),
                        BOX_WAVEFORM_TAG => BoxMatch::Waveform(c0),
                        BOX_FFUN_TAG => BoxMatch::FFun(c0),
                        BOX_CASE_TAG => BoxMatch::Case(c0),
                        BOX_PATTERN_MATCHER_TAG => BoxMatch::PatternMatcher(c0),
                        BOX_CLOSURE_TAG => BoxMatch::Closure(c0),
                        BOX_PATTERN_VAR_TAG => BoxMatch::PatternVar(c0),
                        BOX_INPUTS_TAG => BoxMatch::Inputs(c0),
                        BOX_OUTPUTS_TAG => BoxMatch::Outputs(c0),
                        BOX_CINPUTS_TAG => BoxMatch::CInputs(c0),
                        BOX_COUTPUTS_TAG => BoxMatch::COutputs(c0),
                        BOX_ONDEMAND_TAG => BoxMatch::Ondemand(c0),
                        BOX_UPSAMPLING_TAG => BoxMatch::Upsampling(c0),
                        BOX_DOWNSAMPLING_TAG => BoxMatch::Downsampling(c0),
                        BOX_BUTTON_TAG => BoxMatch::Button(c0),
                        BOX_CHECKBOX_TAG => BoxMatch::Checkbox(c0),
                        _ => BoxMatch::Unknown,
                    }
                }
                2 => {
                    let c0 = children[0];
                    let c1 = children[1];
                    match tag {
                        BOX_SEQ_TAG => BoxMatch::Seq(c0, c1),
                        BOX_PAR_TAG => BoxMatch::Par(c0, c1),
                        BOX_REC_TAG => BoxMatch::Rec(c0, c1),
                        BOX_SPLIT_TAG => BoxMatch::Split(c0, c1),
                        BOX_MERGE_TAG => BoxMatch::Merge(c0, c1),
                        BOX_APPL_TAG => BoxMatch::Appl(c0, c1),
                        BOX_ACCESS_TAG => BoxMatch::Access(c0, c1),
                        BOX_SYMBOLIC_TAG => BoxMatch::Symbolic(c0, c1),
                        BOX_CINPUT_TAG => BoxMatch::CInput(c0, c1),
                        BOX_COUTPUT_TAG => BoxMatch::COutput(c0, c1),
                        BOX_WITH_LOCAL_DEF_TAG => BoxMatch::WithLocalDef(c0, c1),
                        BOX_MODIF_LOCAL_DEF_TAG => BoxMatch::ModifLocalDef(c0, c1),
                        BOX_METADATA_TAG => BoxMatch::Metadata(c0, c1),
                        BOX_ABSTR_TAG => BoxMatch::Abstr(c0, c1),
                        BOX_MODULATION_TAG => BoxMatch::Modulation(c0, c1),
                        BOX_VGROUP_TAG => BoxMatch::VGroup(c0, c1),
                        BOX_HGROUP_TAG => BoxMatch::HGroup(c0, c1),
                        BOX_TGROUP_TAG => BoxMatch::TGroup(c0, c1),
                        BOX_SOUNDFILE_TAG => BoxMatch::Soundfile(c0, c1),
                        BOX_FORWARD_AD_TAG => BoxMatch::ForwardAD(c0, c1),
                        BOX_REVERSE_AD_TAG => BoxMatch::ReverseAD(c0, c1),
                        BOX_VSLIDER_TAG => {
                            let Some((cur, min, max, step)) = slider_params4(arena, c1) else {
                                return BoxMatch::Unknown;
                            };
                            BoxMatch::VSlider(c0, cur, min, max, step)
                        }
                        BOX_HSLIDER_TAG => {
                            let Some((cur, min, max, step)) = slider_params4(arena, c1) else {
                                return BoxMatch::Unknown;
                            };
                            BoxMatch::HSlider(c0, cur, min, max, step)
                        }
                        BOX_NUM_ENTRY_TAG => {
                            let Some((cur, min, max, step)) = slider_params4(arena, c1) else {
                                return BoxMatch::Unknown;
                            };
                            BoxMatch::NumEntry(c0, cur, min, max, step)
                        }
                        _ => BoxMatch::Unknown,
                    }
                }
                3 => {
                    let c0 = children[0];
                    let c1 = children[1];
                    let c2 = children[2];
                    match tag {
                        BOX_IPAR_TAG => BoxMatch::IPar(c0, c1, c2),
                        BOX_ISEQ_TAG => BoxMatch::ISeq(c0, c1, c2),
                        BOX_ISUM_TAG => BoxMatch::ISum(c0, c1, c2),
                        BOX_IPROD_TAG => BoxMatch::IProd(c0, c1, c2),
                        BOX_WITH_REC_DEF_TAG => BoxMatch::WithRecDef(c0, c1, c2),
                        BOX_ROUTE_TAG => BoxMatch::Route(c0, c1, c2),
                        FFUN_TAG => BoxMatch::Ffunction(c0, c1, c2),
                        BOX_FCONST_TAG => BoxMatch::FConst(c0, c1, c2),
                        BOX_FVAR_TAG => BoxMatch::FVar(c0, c1, c2),
                        BOX_VBARGRAPH_TAG => BoxMatch::VBargraph(c0, c1, c2),
                        BOX_HBARGRAPH_TAG => BoxMatch::HBargraph(c0, c1, c2),
                        _ => BoxMatch::Unknown,
                    }
                }
                _ => BoxMatch::Unknown,
            }
        }
        _ => BoxMatch::Unknown,
    }
}
