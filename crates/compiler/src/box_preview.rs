//! Compact box expression previews for diagnostics notes.
//!
//! Provides bounded single-line summaries of box subtrees for use in
//! diagnostic `note` fields where a full tree dump would be too verbose:
//! - `compact_box_preview` — raw dump capped at 180 characters;
//! - `compact_human_box_preview` — human-oriented infix rendering capped at 180 characters;
//! - `render_human_box_expr` — recursive Faust-like expression renderer;
//! - `prim_infix_symbol` / `prim_readable_name` — symbol and name helpers for
//!   primitive box families.

use super::*;

// ─── Box preview helpers ──────────────────────────────────────────────────────

/// Compacts one box subtree dump to a bounded single-line preview for diagnostics notes.
pub(crate) fn compact_box_preview(arena: &tlib::TreeArena, node: BoxId) -> String {
    let preview = dump_box(arena, node);
    let mut one_line = preview.split_whitespace().collect::<Vec<_>>().join(" ");
    const MAX_CHARS: usize = 180;
    if one_line.chars().count() > MAX_CHARS {
        one_line = one_line.chars().take(MAX_CHARS).collect::<String>() + "...";
    }
    one_line
}

/// Compacts one readable box expression preview to a bounded single-line note payload.
pub(crate) fn compact_human_box_preview(arena: &tlib::TreeArena, node: BoxId) -> String {
    let mut rendered = render_human_box_expr(arena, node, 0);
    const MAX_CHARS: usize = 180;
    if rendered.chars().count() > MAX_CHARS {
        rendered = rendered.chars().take(MAX_CHARS).collect::<String>() + "...";
    }
    rendered
}

/// Renders one box subtree to a human-oriented Faust-like expression string.
///
/// The output intentionally trades completeness for readability: composite
/// boxes are rendered as infix expressions when possible, and unknown shapes
/// fall back to a compact [`compact_box_preview`].
///
/// Recursion is bounded at depth 96 to prevent stack overflow on pathological
/// or cyclically-aliased box graphs; deeper sub-trees are replaced with `"..."`.
pub(crate) fn render_human_box_expr(arena: &tlib::TreeArena, node: BoxId, depth: usize) -> String {
    if depth > 96 {
        return "...".to_owned();
    }

    if let Some(kind) = arena.kind(node) {
        match kind {
            NodeKind::StringLiteral(s) => return format!("\"{}\"", s),
            NodeKind::Symbol(s) => return s.to_string(),
            _ => {}
        }
    }

    match match_box(arena, node) {
        BoxMatch::Wire => "_".to_owned(),
        BoxMatch::Cut => "!".to_owned(),
        BoxMatch::Ident(name) => name.to_owned(),
        BoxMatch::Int(v) => v.to_string(),
        BoxMatch::Real(v) => v.to_string(),
        BoxMatch::Par(left, right) => format!(
            "({}, {})",
            render_human_box_expr(arena, left, depth + 1),
            render_human_box_expr(arena, right, depth + 1)
        ),
        BoxMatch::Seq(left, right) => {
            if let BoxMatch::Par(lhs, rhs) = match_box(arena, left)
                && let Some(op) = prim_infix_symbol(arena, right)
            {
                return format!(
                    "({} {} {})",
                    render_human_box_expr(arena, lhs, depth + 1),
                    op,
                    render_human_box_expr(arena, rhs, depth + 1)
                );
            }
            format!(
                "({} : {})",
                render_human_box_expr(arena, left, depth + 1),
                render_human_box_expr(arena, right, depth + 1)
            )
        }
        BoxMatch::Split(left, right) => format!(
            "({} <: {})",
            render_human_box_expr(arena, left, depth + 1),
            render_human_box_expr(arena, right, depth + 1)
        ),
        BoxMatch::Merge(left, right) => format!(
            "({} :> {})",
            render_human_box_expr(arena, left, depth + 1),
            render_human_box_expr(arena, right, depth + 1)
        ),
        BoxMatch::Rec(left, right) => format!(
            "({} ~ {})",
            render_human_box_expr(arena, left, depth + 1),
            render_human_box_expr(arena, right, depth + 1)
        ),
        BoxMatch::Button(label) => {
            format!("button({})", render_human_box_expr(arena, label, depth + 1))
        }
        BoxMatch::Checkbox(label) => {
            format!(
                "checkbox({})",
                render_human_box_expr(arena, label, depth + 1)
            )
        }
        BoxMatch::VSlider(label, cur, min, max, step) => format!(
            "vslider({}, {}, {}, {}, {})",
            render_human_box_expr(arena, label, depth + 1),
            render_human_box_expr(arena, cur, depth + 1),
            render_human_box_expr(arena, min, depth + 1),
            render_human_box_expr(arena, max, depth + 1),
            render_human_box_expr(arena, step, depth + 1)
        ),
        BoxMatch::HSlider(label, cur, min, max, step) => format!(
            "hslider({}, {}, {}, {}, {})",
            render_human_box_expr(arena, label, depth + 1),
            render_human_box_expr(arena, cur, depth + 1),
            render_human_box_expr(arena, min, depth + 1),
            render_human_box_expr(arena, max, depth + 1),
            render_human_box_expr(arena, step, depth + 1)
        ),
        BoxMatch::NumEntry(label, cur, min, max, step) => format!(
            "nentry({}, {}, {}, {}, {})",
            render_human_box_expr(arena, label, depth + 1),
            render_human_box_expr(arena, cur, depth + 1),
            render_human_box_expr(arena, min, depth + 1),
            render_human_box_expr(arena, max, depth + 1),
            render_human_box_expr(arena, step, depth + 1)
        ),
        BoxMatch::VBargraph(label, min, max) => format!(
            "vbargraph({}, {}, {})",
            render_human_box_expr(arena, label, depth + 1),
            render_human_box_expr(arena, min, depth + 1),
            render_human_box_expr(arena, max, depth + 1)
        ),
        BoxMatch::HBargraph(label, min, max) => format!(
            "hbargraph({}, {}, {})",
            render_human_box_expr(arena, label, depth + 1),
            render_human_box_expr(arena, min, depth + 1),
            render_human_box_expr(arena, max, depth + 1)
        ),
        BoxMatch::CInputs(expr) => {
            format!("cinputs({})", render_human_box_expr(arena, expr, depth + 1))
        }
        BoxMatch::COutputs(expr) => {
            format!(
                "coutputs({})",
                render_human_box_expr(arena, expr, depth + 1)
            )
        }
        BoxMatch::CInput(index, expr) => format!(
            "cinput({}, {})",
            render_human_box_expr(arena, index, depth + 1),
            render_human_box_expr(arena, expr, depth + 1)
        ),
        BoxMatch::COutput(index, expr) => format!(
            "coutput({}, {})",
            render_human_box_expr(arena, index, depth + 1),
            render_human_box_expr(arena, expr, depth + 1)
        ),
        BoxMatch::VGroup(label, expr) => format!(
            "vgroup({}, {})",
            render_human_box_expr(arena, label, depth + 1),
            render_human_box_expr(arena, expr, depth + 1)
        ),
        BoxMatch::HGroup(label, expr) => format!(
            "hgroup({}, {})",
            render_human_box_expr(arena, label, depth + 1),
            render_human_box_expr(arena, expr, depth + 1)
        ),
        BoxMatch::TGroup(label, expr) => format!(
            "tgroup({}, {})",
            render_human_box_expr(arena, label, depth + 1),
            render_human_box_expr(arena, expr, depth + 1)
        ),
        BoxMatch::Soundfile(label, chan) => format!(
            "soundfile({}, {})",
            render_human_box_expr(arena, label, depth + 1),
            render_human_box_expr(arena, chan, depth + 1)
        ),
        BoxMatch::Add
        | BoxMatch::Sub
        | BoxMatch::Mul
        | BoxMatch::Div
        | BoxMatch::Rem
        | BoxMatch::And
        | BoxMatch::Or
        | BoxMatch::Xor
        | BoxMatch::Lsh
        | BoxMatch::Rsh
        | BoxMatch::Lt
        | BoxMatch::Le
        | BoxMatch::Gt
        | BoxMatch::Ge
        | BoxMatch::Eq
        | BoxMatch::Ne
        | BoxMatch::Pow
        | BoxMatch::Delay
        | BoxMatch::Delay1
        | BoxMatch::Min
        | BoxMatch::Max
        | BoxMatch::Acos
        | BoxMatch::Asin
        | BoxMatch::Atan
        | BoxMatch::Atan2
        | BoxMatch::Cos
        | BoxMatch::Sin
        | BoxMatch::Tan
        | BoxMatch::Exp
        | BoxMatch::Exp10
        | BoxMatch::Log
        | BoxMatch::Log10
        | BoxMatch::Sqrt
        | BoxMatch::Abs
        | BoxMatch::Fmod
        | BoxMatch::Remainder
        | BoxMatch::Floor
        | BoxMatch::Ceil
        | BoxMatch::Rint
        | BoxMatch::Round
        | BoxMatch::Prefix
        | BoxMatch::IntCast
        | BoxMatch::FloatCast
        | BoxMatch::ReadOnlyTable
        | BoxMatch::WriteReadTable
        | BoxMatch::Select2
        | BoxMatch::Select3
        | BoxMatch::AssertBounds
        | BoxMatch::Lowest
        | BoxMatch::Highest
        | BoxMatch::Attach
        | BoxMatch::Enable
        | BoxMatch::Control => prim_infix_symbol(arena, node)
            .or_else(|| prim_readable_name(arena, node))
            .unwrap_or("?")
            .to_owned(),
        _ => compact_box_preview(arena, node),
    }
}

/// Maps a primitive box node to its Faust infix operator symbol.
///
/// Returns `None` for primitives that are not infix operators (e.g. prefix
/// or postfix forms). Used by [`render_human_box_expr`] to produce readable
/// `A + B`-style diagnostic strings rather than box-type names.
pub(crate) fn prim_infix_symbol(arena: &tlib::TreeArena, node: BoxId) -> Option<&'static str> {
    match match_box(arena, node) {
        BoxMatch::Add => Some("+"),
        BoxMatch::Sub => Some("-"),
        BoxMatch::Mul => Some("*"),
        BoxMatch::Div => Some("/"),
        BoxMatch::Rem => Some("%"),
        BoxMatch::Pow => Some("^"),
        BoxMatch::Lt => Some("<"),
        BoxMatch::Le => Some("<="),
        BoxMatch::Gt => Some(">"),
        BoxMatch::Ge => Some(">="),
        BoxMatch::Eq => Some("=="),
        BoxMatch::Ne => Some("!="),
        BoxMatch::And => Some("&"),
        BoxMatch::Or => Some("|"),
        BoxMatch::Xor => Some("xor"),
        BoxMatch::Lsh => Some("<<"),
        BoxMatch::Rsh => Some(">>"),
        _ => None,
    }
}

/// Returns one readable primitive name for non-infix `BoxMatch` primitive nodes.
pub(crate) fn prim_readable_name(arena: &tlib::TreeArena, node: BoxId) -> Option<&'static str> {
    match match_box(arena, node) {
        BoxMatch::Delay => Some("@"),
        BoxMatch::Delay1 => Some("'"),
        BoxMatch::Min => Some("min"),
        BoxMatch::Max => Some("max"),
        BoxMatch::Acos => Some("acos"),
        BoxMatch::Asin => Some("asin"),
        BoxMatch::Atan => Some("atan"),
        BoxMatch::Atan2 => Some("atan2"),
        BoxMatch::Cos => Some("cos"),
        BoxMatch::Sin => Some("sin"),
        BoxMatch::Tan => Some("tan"),
        BoxMatch::Exp => Some("exp"),
        BoxMatch::Exp10 => Some("exp10"),
        BoxMatch::Log => Some("log"),
        BoxMatch::Log10 => Some("log10"),
        BoxMatch::Sqrt => Some("sqrt"),
        BoxMatch::Abs => Some("abs"),
        BoxMatch::Fmod => Some("fmod"),
        BoxMatch::Remainder => Some("remainder"),
        BoxMatch::Floor => Some("floor"),
        BoxMatch::Ceil => Some("ceil"),
        BoxMatch::Rint => Some("rint"),
        BoxMatch::Round => Some("round"),
        BoxMatch::Prefix => Some("prefix"),
        BoxMatch::IntCast => Some("int"),
        BoxMatch::FloatCast => Some("float"),
        BoxMatch::ReadOnlyTable => Some("rdtable"),
        BoxMatch::WriteReadTable => Some("rwtable"),
        BoxMatch::Select2 => Some("select2"),
        BoxMatch::Select3 => Some("select3"),
        BoxMatch::AssertBounds => Some("assertbounds"),
        BoxMatch::Lowest => Some("lowest"),
        BoxMatch::Highest => Some("highest"),
        BoxMatch::Attach => Some("attach"),
        BoxMatch::Enable => Some("enable"),
        BoxMatch::Control => Some("control"),
        _ => None,
    }
}
