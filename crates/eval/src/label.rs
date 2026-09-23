//! UI and modulation label evaluation.
//!
//! Ports the C++ `evalLabel(...)` mini-parser used to resolve dynamic label
//! strings in widget and modulation expressions:
//! - `eval_label_node` — evaluates a label tree node to a `String`;
//! - `eval_label` — the state-machine parser that substitutes `%` and `%i`
//!   placeholders by evaluating identifiers from the current environment;
//! - `is_eval_label_ident_char` / `write_label_ident_value` — character
//!   classification and substitution helpers;
//! - `strip_label_*` / `label_node_text` / `is_subsequence` — utility helpers
//!   for label text extraction and suffix matching.
//!
//! Source provenance (C++): `compiler/evaluate/eval.cpp` — `evalLabel(...)`,
//! `writeIdentValue(...)`.

use super::*;

/// Evaluates one UI/modulation label node using the C++ `evalLabel(...)`
/// placeholder semantics.
///
/// Source provenance (C++):
/// - `compiler/evaluate/eval.cpp`
/// - `evalLabel(...)`
/// - `writeIdentValue(...)`
///
/// Mapping status: `adapted`.
/// Rust mirrors the C++ label substitution state machine while resolving
/// placeholder values through explicit evaluator helpers instead of global tree
/// properties.
pub(crate) fn eval_label_node(
    arena: &mut TreeArena,
    label_node: TreeId,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<String, EvalError> {
    let Some(src) = label_node_text(arena, label_node) else {
        return Err(EvalError::InvalidModulationLabel { node: label_node });
    };
    let src = src.to_owned();
    eval_label(arena, &src, env, loop_detector)
}

/// Port of the C++ `evalLabel(...)` mini-parser used for dynamic UI labels.
pub(crate) fn eval_label(
    arena: &mut TreeArena,
    src: &str,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<String, EvalError> {
    #[derive(Clone, Copy)]
    enum State {
        Text,
        AfterPercent,
        Ident,
        BracedIdent,
    }

    let chars: Vec<char> = src.chars().collect();
    let mut idx = 0usize;
    let mut state = State::Text;
    let mut dst = String::new();
    let mut ident = String::new();
    let mut format = String::new();

    while idx <= chars.len() {
        let cur = chars.get(idx).copied();
        match state {
            State::Text => match cur {
                None => break,
                Some('%') => {
                    ident.clear();
                    format.clear();
                    state = State::AfterPercent;
                    idx += 1;
                }
                Some(ch) => {
                    dst.push(ch);
                    idx += 1;
                }
            },
            State::AfterPercent => match cur {
                None => {
                    dst.push('%');
                    dst.push_str(&format);
                    break;
                }
                Some(ch) if ch.is_ascii_digit() => {
                    format.push(ch);
                    idx += 1;
                }
                Some(ch) if is_eval_label_ident_char(ch) => {
                    ident.push(ch);
                    state = State::Ident;
                    idx += 1;
                }
                Some('{') => {
                    state = State::BracedIdent;
                    idx += 1;
                }
                Some(_) => {
                    dst.push('%');
                    dst.push_str(&format);
                    state = State::Text;
                }
            },
            State::Ident => match cur {
                Some(ch) if is_eval_label_ident_char(ch) => {
                    ident.push(ch);
                    idx += 1;
                }
                _ => {
                    write_label_ident_value(arena, &mut dst, &format, &ident, env, loop_detector)?;
                    state = State::Text;
                }
            },
            State::BracedIdent => match cur {
                Some(ch) if is_eval_label_ident_char(ch) => {
                    ident.push(ch);
                    idx += 1;
                }
                Some('}') => {
                    write_label_ident_value(arena, &mut dst, &format, &ident, env, loop_detector)?;
                    idx += 1;
                    state = State::Text;
                }
                _ => {
                    dst.push('%');
                    dst.push_str(&format);
                    break;
                }
            },
        }
    }

    Ok(dst)
}

/// Returns `true` for identifier characters accepted by `%ident` label syntax.
///
/// This intentionally follows the conservative subset used by the current Rust
/// port of `evalLabel(...)`: ASCII alphanumerics plus `_`.
pub(crate) fn is_eval_label_ident_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

/// Renders one `%ident` or `%{ident}` placeholder into the destination label.
///
/// Width formatting follows the C++ `evalLabel(...)` convention implemented by
/// the active corpus: the optional decimal field width is clamped to `0..=4`
/// before rendering the resolved integer value.
pub(crate) fn write_label_ident_value(
    arena: &mut TreeArena,
    dst: &mut String,
    format: &str,
    ident: &str,
    env: &Environment,
    loop_detector: &mut LoopDetector,
) -> Result<(), EvalError> {
    let width = format.parse::<usize>().unwrap_or(0).clamp(0, 4);
    let value = eval_ident_to_constant_int(arena, ident, env, loop_detector)?;
    let rendered = if width == 0 {
        value.to_string()
    } else {
        format!("{value:>width$}")
    };
    dst.push_str(&rendered);
    Ok(())
}

/// Extracts the plain-text label content from one label node.
///
/// Missing/invalid label nodes degrade to an empty string so modulation path
/// reconstruction stays total during recursive traversal.
pub(crate) fn strip_label_node(arena: &TreeArena, label: TreeId) -> String {
    label_node_text(arena, label)
        .map(strip_label_metadata)
        .unwrap_or_default()
}

/// Removes every Faust metadata declaration from one textual label.
///
/// `gain [unit:dB]`, `[1] gain` and `[1] gain [tooltip: ...]` all become
/// `gain`, trimmed of spaces and tabs: the label the UI shows, and the one a
/// modulation target is matched against. The extraction is the `ui` crate's
/// port of the reference `extractMetadata` (escapes, nested brackets), so a
/// label that starts with a metadata declaration keeps its text, where a cut
/// at the first `[` would leave nothing.
///
/// Source provenance (C++): `removeMetadata` in `compiler/generator/description.cpp`,
/// applied by `superNormalizePath` (`compiler/propagate/labels.cpp`).
pub(crate) fn strip_label_metadata(label: &str) -> String {
    ui::split_label_metadata(label).0
}

/// The path a widget's own label declares: the label first, then the groups
/// the label opens, innermost first, every segment without its metadata.
///
/// `"h:sub/x [unit:Hz]"` is `["x", "sub"]`, `"x"` is `["x"]`. A label with a
/// `/` but no `h:`/`v:`/`t:` prefix opens no group and stays one segment, as
/// in the reference compiler. Root and parent navigation (`/x`, `../x`) has
/// no enclosing path to act on here and is dropped.
///
/// Source provenance (C++): `superNormalizePath(cons(wLabel, nil))` in
/// `implantWidgetIfMatch` (`compiler/transform/boxModulationImplanter.cpp`),
/// through `label2path` (`compiler/propagate/labels.cpp`).
pub(crate) fn widget_label_path_segments(label: &str) -> Vec<String> {
    let normalized = ui::normalize_widget_label_path(label, &[]);
    let mut segments = Vec::with_capacity(normalized.groups.len() + 1);
    segments.push(strip_label_metadata(&normalized.raw_label));
    for group in normalized.groups.iter().rev() {
        segments.push(strip_label_metadata(&group.raw_label));
    }
    segments
}

/// Returns the raw textual payload of a label node, if any.
///
/// Both string literals and interned symbols are accepted to stay compatible
/// with transitional tree encodings.
pub(crate) fn label_node_text(arena: &TreeArena, label: TreeId) -> Option<&str> {
    match arena.kind(label) {
        Some(NodeKind::StringLiteral(label)) => Some(label.as_ref()),
        Some(NodeKind::Symbol(label)) => Some(label.as_ref()),
        _ => None,
    }
}

/// Returns `true` when `needle` appears in-order inside `haystack`.
///
/// This relaxed path relation is used by the current modulation implementation
/// so target paths can match inside nested UI groups without requiring exact
/// absolute-path equality.
pub(crate) fn is_subsequence(needle: &[String], haystack: &[String]) -> bool {
    let mut haystack_iter = haystack.iter();
    needle
        .iter()
        .all(|target| haystack_iter.by_ref().any(|candidate| candidate == target))
}
